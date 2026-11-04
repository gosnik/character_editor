//! Native reader for Ara's small MHP1 asset containers.
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

const MAX_PACK: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Array {
    F32(Vec<f32>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U8(Vec<u8>),
}
impl Array {
    pub fn f32(&self) -> Result<&[f32], String> {
        if let Self::F32(x) = self {
            Ok(x)
        } else {
            Err("array is not Float32Array".into())
        }
    }
    pub fn u32(&self) -> Result<&[u32], String> {
        if let Self::U32(x) = self {
            Ok(x)
        } else {
            Err("array is not Uint32Array".into())
        }
    }
    pub fn u16(&self) -> Result<&[u16], String> {
        if let Self::U16(x) = self {
            Ok(x)
        } else {
            Err("array is not Uint16Array".into())
        }
    }
    pub fn i16(&self) -> Result<&[i16], String> {
        if let Self::I16(x) = self {
            Ok(x)
        } else {
            Err("array is not Int16Array".into())
        }
    }
    pub fn u8(&self) -> Result<&[u8], String> {
        if let Self::U8(x) = self {
            Ok(x)
        } else {
            Err("array is not Uint8Array".into())
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::F32(x) => x.len(),
            Self::U32(x) => x.len(),
            Self::U16(x) => x.len(),
            Self::I16(x) => x.len(),
            Self::U8(x) => x.len(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Pack {
    pub meta: Value,
    pub arrays: BTreeMap<String, Array>,
}
#[derive(Clone, Debug)]
pub struct NativeLibrary {
    pub base: Pack,
    pub targets: Pack,
    pub catalog: Value,
    pub root: PathBuf,
    cartoon_pack: Arc<OnceLock<Result<Pack, String>>>,
}

impl NativeLibrary {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref().to_path_buf();
        let catalog: Value = serde_json::from_slice(
            &fs::read(root.join("catalog.json")).map_err(|e| format!("catalog.json: {e}"))?,
        )
        .map_err(|e| format!("catalog.json: {e}"))?;
        let base =
            decode_pack(&fs::read(root.join("base.pack")).map_err(|e| format!("base.pack: {e}"))?)?;
        let targets = decode_targets(decode_pack(
            &fs::read(root.join("targets.pack")).map_err(|e| format!("targets.pack: {e}"))?,
        )?)?;
        Ok(Self {
            base,
            targets,
            catalog,
            root,
            cartoon_pack: Default::default(),
        })
    }
    /// Library from already-decoded packs (tests and embedders).
    pub fn from_packs(base: Pack, targets: Pack, catalog: Value, root: PathBuf) -> Self {
        Self {
            base,
            targets,
            catalog,
            root,
            cartoon_pack: Default::default(),
        }
    }
    /// Cartoon hm08 offsets and expression corrections (`cartoon_hm08/cartoon.pack`,
    /// built by `tools/cartoon_hm08/build.py`).  Format-2 targets on hm08 numbering,
    /// added on top of the regular targets by the Cartoon amount.  Loaded once.
    pub fn cartoon_offsets(&self) -> Result<&Pack, String> {
        self.cartoon_pack
            .get_or_init(|| {
                let path = self.root.join("cartoon_hm08/cartoon.pack");
                decode_targets(decode_pack(
                    &fs::read(&path).map_err(|e| format!("cartoon_hm08/cartoon.pack: {e}"))?,
                )?)
            })
            .as_ref()
            .map_err(Clone::clone)
    }
    /// Loads a proxy on demand; callers may cache returned packs if desired.
    pub fn proxy(&self, relative: &str) -> Result<Pack, String> {
        let p = Path::new(relative);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("invalid proxy path".into());
        }
        decode_pack(&fs::read(self.root.join(p)).map_err(|e| format!("proxy: {e}"))?)
    }
}

fn bytes(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() >= 2 && input[0] == 0x1f && input[1] == 0x8b {
        let mut d = GzDecoder::new(input);
        let mut o = Vec::new();
        d.by_ref()
            .take((MAX_PACK + 1) as u64)
            .read_to_end(&mut o)
            .map_err(|e| format!("gzip: {e}"))?;
        if o.len() > MAX_PACK {
            return Err("decompressed pack exceeds limit".into());
        };
        Ok(o)
    } else {
        if input.len() > MAX_PACK {
            return Err("pack exceeds limit".into());
        };
        Ok(input.to_vec())
    }
}
fn le32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .ok_or_else(|| "truncated pack".to_string())
        .map(|x| u32::from_le_bytes(x.try_into().unwrap()))
}
pub fn decode_pack(input: &[u8]) -> Result<Pack, String> {
    let b = bytes(input)?;
    if b.len() < 8 || le32(&b, 0)? != 0x3150_484d {
        return Err("not an MHP1 pack".into());
    };
    let h = le32(&b, 4)? as usize;
    let end = 8usize
        .checked_add(h)
        .filter(|x| *x <= b.len())
        .ok_or("invalid header length")?;
    let header: Value =
        serde_json::from_slice(&b[8..end]).map_err(|e| format!("pack header: {e}"))?;
    let meta = header.get("meta").cloned().unwrap_or(Value::Null);
    let entries = header
        .get("arrays")
        .and_then(Value::as_array)
        .ok_or("pack header arrays missing")?;
    let data = &b[end..];
    let mut arrays = BTreeMap::new();
    for e in entries {
        let o = e.as_object().ok_or("array entry not object")?;
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("array name missing")?
            .to_owned();
        if arrays.contains_key(&name) {
            return Err("duplicate array name".into());
        };
        let ty = o
            .get("type")
            .and_then(Value::as_str)
            .ok_or("array type missing")?;
        let off = o
            .get("offset")
            .and_then(Value::as_u64)
            .ok_or("array offset missing")? as usize;
        let n = o
            .get("length")
            .and_then(Value::as_u64)
            .ok_or("array length missing")? as usize;
        let sz = match ty {
            "Float32Array" | "Uint32Array" => 4,
            "Uint16Array" | "Int16Array" => 2,
            "Uint8Array" => 1,
            _ => return Err(format!("unsupported array type {ty}")),
        };
        let nbytes = n.checked_mul(sz).ok_or("array length overflow")?;
        let s = data
            .get(off..off.checked_add(nbytes).ok_or("array end overflow")?)
            .ok_or("array out of bounds")?;
        let a = match ty {
            "Float32Array" => Array::F32(
                s.chunks_exact(4)
                    .map(|x| f32::from_le_bytes(x.try_into().unwrap()))
                    .collect(),
            ),
            "Uint32Array" => Array::U32(
                s.chunks_exact(4)
                    .map(|x| u32::from_le_bytes(x.try_into().unwrap()))
                    .collect(),
            ),
            "Uint16Array" => Array::U16(
                s.chunks_exact(2)
                    .map(|x| u16::from_le_bytes(x.try_into().unwrap()))
                    .collect(),
            ),
            "Int16Array" => Array::I16(
                s.chunks_exact(2)
                    .map(|x| i16::from_le_bytes(x.try_into().unwrap()))
                    .collect(),
            ),
            _ => Array::U8(s.to_vec()),
        };
        arrays.insert(name, a);
    }
    Ok(Pack { meta, arrays })
}
fn decode_targets(mut p: Pack) -> Result<Pack, String> {
    if p.meta.get("format").and_then(Value::as_u64) != Some(2) {
        return Ok(p);
    };
    let idx = p.arrays.remove("idx").ok_or("targets idx missing")?;
    let idx = match idx {
        Array::U16(x) => x,
        _ => return Err("targets idx must be Uint16Array".into()),
    };
    let (xlo, xhi, ylo, yhi, zlo, zhi) = (
        p.arrays.remove("xlo").ok_or("xlo missing")?,
        p.arrays.remove("xhi").ok_or("xhi missing")?,
        p.arrays.remove("ylo").ok_or("ylo missing")?,
        p.arrays.remove("yhi").ok_or("yhi missing")?,
        p.arrays.remove("zlo").ok_or("zlo missing")?,
        p.arrays.remove("zhi").ok_or("zhi missing")?,
    );
    let planes = [
        xlo.u8()?,
        xhi.u8()?,
        ylo.u8()?,
        yhi.u8()?,
        zlo.u8()?,
        zhi.u8()?,
    ];
    if planes.iter().any(|x| x.len() != idx.len()) {
        return Err("target planes have unequal length".into());
    };
    let table = p
        .meta
        .get("targets")
        .and_then(Value::as_object)
        .ok_or("targets table missing")?;
    let mut out = [
        vec![0i16; idx.len()],
        vec![0i16; idx.len()],
        vec![0i16; idx.len()],
    ];
    for v in table.values() {
        let a = v
            .as_array()
            .filter(|x| x.len() >= 2)
            .ok_or("invalid target entry")?;
        let off = a[0].as_u64().ok_or("invalid target offset")? as usize;
        let n = a[1].as_u64().ok_or("invalid target count")? as usize;
        let e = off
            .checked_add(n)
            .filter(|x| *x <= idx.len())
            .ok_or("target range out of bounds")?;
        for d in 0..3 {
            let mut cur = 0u16;
            for i in off..e {
                cur = cur
                    .wrapping_add((planes[d * 2][i] as u16) | ((planes[d * 2 + 1][i] as u16) << 8));
                out[d][i] = cur as i16;
            }
        }
    }
    p.arrays.insert("idx".into(), Array::U16(idx));
    p.arrays
        .insert("dx".into(), Array::I16(std::mem::take(&mut out[0])));
    p.arrays
        .insert("dy".into(), Array::I16(std::mem::take(&mut out[1])));
    p.arrays
        .insert("dz".into(), Array::I16(std::mem::take(&mut out[2])));
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_little_endian_typed_arrays_and_rejects_bad_ranges() {
        let h = br#"{"meta":{"ok":true},"arrays":[{"name":"v","type":"Uint16Array","offset":0,"length":2}]}"#;
        let pad = (h.len() + 3) & !3;
        let mut p = Vec::new();
        p.extend_from_slice(&0x3150_484du32.to_le_bytes());
        p.extend_from_slice(&(pad as u32).to_le_bytes());
        p.extend_from_slice(h);
        p.resize(8 + pad, b' ');
        p.extend_from_slice(&7u16.to_le_bytes());
        p.extend_from_slice(&9u16.to_le_bytes());
        assert_eq!(decode_pack(&p).unwrap().arrays["v"].u16().unwrap(), &[7, 9]);
        p.pop();
        assert!(decode_pack(&p).is_err());
    }
}
