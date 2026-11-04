//! MakeHuman morph lattice and fitted-proxy evaluation, without a JavaScript runtime.
use crate::native_assets::{Array, NativeLibrary, Pack};
use serde_json::Value;
use std::collections::BTreeMap;

type Weights = Vec<(&'static str, f32)>;
fn number(p: &Value, key: &str, default: f32) -> f32 {
    p.get(key)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .map(|x| x as f32)
        .unwrap_or(default)
}
fn tri(v: f32, lo: &'static str, mid: &'static str, hi: &'static str) -> Weights {
    if v < 0.5 {
        vec![(lo, 1. - v * 2.), (mid, v * 2.)]
    } else {
        vec![(mid, 2. - v * 2.), (hi, v * 2. - 1.)]
    }
}
fn add(out: &mut BTreeMap<String, f32>, name: String, w: f32) {
    if w.abs() > 1e-5 {
        *out.entry(name).or_default() += w
    }
}

/// Exact port of Ara's `targetWeights`; absent parameters use Ara's defaults.
pub fn target_weights(params: &Value, catalog: &Value) -> BTreeMap<String, f32> {
    let gender = number(params, "gender", 1.);
    let age = number(params, "age", 0.5);
    let muscle = number(params, "muscle", 0.6);
    let weight = number(params, "weight", 0.5);
    let height = number(params, "height", 0.5);
    let proportions = number(params, "proportions", 0.7);
    let african = number(params, "african", 0.1);
    let asian = number(params, "asian", 0.1);
    let caucasian = number(params, "caucasian", 0.8);
    let breast_size = number(params, "breastSize", 0.5);
    let breast_firmness = number(params, "breastFirmness", 0.5);
    let g = vec![("female", 1. - gender), ("male", gender)];
    let a = if age < 0.5 {
        let young = (age - 0.1875).mul_add(3.2, 0.).max(0.);
        let child = (5.333 * age).clamp(0., 1.) - young;
        vec![
            ("baby", (1. - age * 5.333).max(0.)),
            ("child", child.max(0.)),
            ("young", young),
        ]
    } else {
        let old = (age * 2. - 1.).max(0.);
        vec![("young", 1. - old), ("old", old)]
    };
    let m = tri(muscle, "minmuscle", "averagemuscle", "maxmuscle");
    let wt = tri(weight, "minweight", "averageweight", "maxweight");
    let rs = african + asian + caucasian;
    let r = if rs > 0. {
        vec![
            ("african", african / rs),
            ("asian", asian / rs),
            ("caucasian", caucasian / rs),
        ]
    } else {
        vec![
            ("african", 1. / 3.),
            ("asian", 1. / 3.),
            ("caucasian", 1. / 3.),
        ]
    };
    let cup = tri(breast_size, "mincup", "averagecup", "maxcup");
    let firm = tri(
        breast_firmness,
        "minfirmness",
        "averagefirmness",
        "maxfirmness",
    );
    let hv = height * 2. - 1.;
    let pv = proportions * 2. - 1.;
    let mut out = BTreeMap::new();
    for (gs, gw) in &g {
        for (as_, aw) in &a {
            if gw * aw < 1e-5 {
                continue;
            }
            for (rs_, rw) in &r {
                add(
                    &mut out,
                    format!("macrodetails/{rs_}-{gs}-{as_}"),
                    rw * gw * aw,
                )
            }
            for (ms, mw) in &m {
                for (ws, ww) in &wt {
                    let k = gw * aw * mw * ww;
                    if k < 1e-5 {
                        continue;
                    }
                    add(
                        &mut out,
                        format!("macrodetails/universal-{gs}-{as_}-{ms}-{ws}"),
                        k,
                    );
                    if hv != 0. {
                        add(
                            &mut out,
                            format!(
                                "macrodetails/height/{gs}-{as_}-{ms}-{ws}-{}height",
                                if hv < 0. { "min" } else { "max" }
                            ),
                            k * hv.abs(),
                        )
                    }
                    if pv != 0. {
                        add(
                            &mut out,
                            format!(
                                "macrodetails/proportions/{gs}-{as_}-{ms}-{ws}-{}proportions",
                                if pv < 0. { "uncommon" } else { "ideal" }
                            ),
                            k * pv.abs(),
                        )
                    }
                    if *gs == "female" {
                        for (cs, cw) in &cup {
                            for (fs, fw) in &firm {
                                if !(*cs == "averagecup" && *fs == "averagefirmness") {
                                    add(
                                        &mut out,
                                        format!("breast/{gs}-{as_}-{ms}-{ws}-{cs}-{fs}"),
                                        k * cw * fw,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut sliders = BTreeMap::<&str, &Value>::new();
    if let Some(groups) = catalog.get("sliders").and_then(Value::as_array) {
        for group in groups {
            if let Some(ss) = group.get("sliders").and_then(Value::as_array) {
                for s in ss {
                    if let Some(id) = s.get("id").and_then(Value::as_str) {
                        sliders.insert(id, s);
                    }
                }
            }
        }
    }
    if let Some(mods) = params.get("modifiers").and_then(Value::as_object) {
        for (id, v) in mods {
            let Some(v) = v.as_f64().filter(|x| x.is_finite()).map(|x| x as f32) else {
                continue;
            };
            if v == 0. {
                continue;
            }
            let Some(s) = sliders.get(id.as_str()) else {
                continue;
            };
            let Some(pairs) = s.get("targets").and_then(Value::as_array) else {
                continue;
            };
            for pair in pairs {
                let Some(pair) = pair.as_array().filter(|x| x.len() == 2) else {
                    continue;
                };
                let name = if v < 0. {
                    pair[0].as_str()
                } else {
                    pair[1].as_str()
                };
                if let Some(name) = name {
                    add(&mut out, name.to_owned(), v.abs())
                }
            }
        }
    }
    out
}
fn arr<'a>(p: &'a Pack, n: &str) -> Result<&'a Array, String> {
    p.arrays.get(n).ok_or_else(|| format!("array {n} missing"))
}
/// Applies sparse MakeHuman target deltas. Returned coordinates remain in MakeHuman decimetres.
pub fn apply_targets(lib: &NativeLibrary, params: &Value) -> Result<Vec<[f32; 3]>, String> {
    let weights = target_weights(params, &lib.catalog);
    apply_weighted_targets(lib, &weights)
}
/// Applies an explicit map of target names to weights.  This is used by face
/// morphs too, so all MakeHuman target units share the same checked decoder.
pub fn apply_weighted_targets(
    lib: &NativeLibrary,
    weights: &BTreeMap<String, f32>,
) -> Result<Vec<[f32; 3]>, String> {
    let pos = arr(&lib.base, "positions")?.f32()?;
    if pos.len() % 3 != 0 {
        return Err("positions length is not xyz triples".into());
    };
    let mut out: Vec<[f32; 3]> = pos.chunks_exact(3).map(|x| [x[0], x[1], x[2]]).collect();
    add_targets(&lib.targets, &mut out, weights)?;
    Ok(out)
}
/// Adds weighted format-2 targets from `pack` to `out` in place.
pub fn add_targets(
    pack: &Pack,
    out: &mut [[f32; 3]],
    weights: &BTreeMap<String, f32>,
) -> Result<(), String> {
    let idx = arr(pack, "idx")?.u16()?;
    let dx = arr(pack, "dx")?.i16()?;
    let dy = arr(pack, "dy")?.i16()?;
    let dz = arr(pack, "dz")?.i16()?;
    if [dx.len(), dy.len(), dz.len()]
        .iter()
        .any(|&n| n != idx.len())
    {
        return Err("target delta arrays have unequal length".into());
    };
    let targets = pack
        .meta
        .get("targets")
        .and_then(Value::as_object)
        .ok_or("targets metadata missing")?;
    for (name, &w) in weights {
        let Some(entry) = targets
            .get(name)
            .and_then(Value::as_array)
            .filter(|x| x.len() >= 3)
        else {
            continue;
        };
        let off = entry[0]
            .as_u64()
            .ok_or_else(|| format!("target {name}: bad offset"))? as usize;
        let n = entry[1]
            .as_u64()
            .ok_or_else(|| format!("target {name}: bad count"))? as usize;
        let scale = entry[2]
            .as_f64()
            .filter(|x| x.is_finite())
            .ok_or_else(|| format!("target {name}: bad scale"))? as f32;
        let end = off
            .checked_add(n)
            .filter(|x| *x <= idx.len())
            .ok_or_else(|| format!("target {name}: range out of bounds"))?;
        let k = w * scale;
        let mut v = 0usize;
        for i in off..end {
            v = v
                .checked_add(idx[i] as usize)
                .ok_or("vertex index overflow")?;
            let p = out
                .get_mut(v)
                .ok_or_else(|| format!("target {name}: vertex out of bounds"))?;
            p[0] += dx[i] as f32 * k;
            p[1] += dy[i] as f32 * k;
            p[2] += dz[i] as f32 * k;
        }
    }
    Ok(())
}
/// Fits an Ara proxy through its barycentric references and per-axis scaling anchors.
pub fn fit_proxy(proxy: &Pack, coords: &[[f32; 3]]) -> Result<Vec<[f32; 3]>, String> {
    let refs = arr(proxy, "refs")?.u32()?;
    let weights = arr(proxy, "weights")?.f32()?;
    let offsets = arr(proxy, "offsets")?.f32()?;
    let n = proxy
        .meta
        .get("vertexCount")
        .and_then(Value::as_u64)
        .ok_or("proxy vertexCount missing")? as usize;
    if refs.len() != n.checked_mul(3).ok_or("proxy vertex count overflow")?
        || weights.len() != refs.len()
        || offsets.len() != refs.len()
    {
        return Err("proxy fit arrays do not match vertexCount".into());
    };
    let scale = proxy
        .meta
        .get("scale")
        .and_then(Value::as_array)
        .ok_or("proxy scale missing")?;
    if scale.len() != 3 {
        return Err("proxy scale must have three axes".into());
    };
    let mut sc = [1.; 3];
    for d in 0..3 {
        if scale[d].is_null() {
            continue;
        }
        let a = scale[d]
            .as_array()
            .filter(|x| x.len() == 3)
            .ok_or("invalid proxy scale anchor")?;
        let va = a[0].as_u64().ok_or("invalid scale vertex")? as usize;
        let vb = a[1].as_u64().ok_or("invalid scale vertex")? as usize;
        let den = a[2]
            .as_f64()
            .filter(|x| *x != 0. && x.is_finite())
            .ok_or("invalid scale distance")? as f32;
        sc[d] = (coords.get(va).ok_or("scale vertex out of bounds")?[d]
            - coords.get(vb).ok_or("scale vertex out of bounds")?[d])
            .abs()
            / den;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = coords
            .get(refs[i * 3] as usize)
            .ok_or("proxy ref out of bounds")?;
        let b = coords
            .get(refs[i * 3 + 1] as usize)
            .ok_or("proxy ref out of bounds")?;
        let c = coords
            .get(refs[i * 3 + 2] as usize)
            .ok_or("proxy ref out of bounds")?;
        let (wa, wb, wc) = (weights[i * 3], weights[i * 3 + 1], weights[i * 3 + 2]);
        out.push([
            wa * a[0] + wb * b[0] + wc * c[0] + offsets[i * 3] * sc[0],
            wa * a[1] + wb * b[1] + wc * c[1] + offsets[i * 3 + 1] * sc[1],
            wa * a[2] + wb * b[2] + wc * c[2] + offsets[i * 3 + 2] * sc[2],
        ])
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_assets::Array;
    use serde_json::json;

    #[test]
    fn macro_lattice_and_slider_follow_ara_names_and_weights() {
        let cat = json!({"sliders":[{"sliders":[{"id":"chin","targets":[["chin/lo","chin/hi"],[null,"chin/also"]]}]}]});
        let p = json!({"gender":1.0,"age":0.5,"muscle":0.5,"weight":0.5,"height":0.75,"proportions":0.5,"african":0.0,"asian":0.0,"caucasian":1.0,"modifiers":{"chin":0.25}});
        let w = target_weights(&p, &cat);
        assert_eq!(w.get("macrodetails/caucasian-male-young"), Some(&1.));
        assert_eq!(
            w.get("macrodetails/universal-male-young-averagemuscle-averageweight"),
            Some(&1.)
        );
        assert_eq!(
            w.get("macrodetails/height/male-young-averagemuscle-averageweight-maxheight"),
            Some(&0.5)
        );
        assert_eq!(w.get("chin/hi"), Some(&0.25));
        assert_eq!(w.get("chin/also"), Some(&0.25));
    }

    #[test]
    fn proxy_fit_interpolates_and_scales_offsets() {
        let proxy = Pack {
            meta: json!({"vertexCount":1,"scale":[[0,1,2.0],null,null]}),
            arrays: BTreeMap::from([
                ("refs".into(), Array::U32(vec![0, 1, 2])),
                ("weights".into(), Array::F32(vec![0.25, 0.5, 0.25])),
                ("offsets".into(), Array::F32(vec![1., 2., 3.])),
            ]),
        };
        let p = fit_proxy(&proxy, &[[0., 0., 0.], [4., 2., 0.], [0., 2., 4.]]).unwrap();
        assert_eq!(p, vec![[4., 3.5, 4.]]);
    }
}
