"""Reader/writer for the editor's MHP1 packs (assets/native/*.pack)."""
import gzip, json, struct
from pathlib import Path
import numpy as np

TYPES = {"Float32Array": "<f4", "Uint32Array": "<u4", "Uint16Array": "<u2", "Int16Array": "<i2", "Uint8Array": "u1"}
NAMES = {np.dtype(v).str: k for k, v in TYPES.items()}


def read(path):
    b = Path(path).read_bytes()
    b = gzip.decompress(b) if b[:2] == b"\x1f\x8b" else b
    assert b[:4] == b"MHP1"
    n = struct.unpack_from("<I", b, 4)[0]; h = json.loads(b[8:8 + n]); data = memoryview(b)[8 + n:]
    arrays = {e["name"]: np.frombuffer(data, dtype=TYPES[e["type"]], count=e["length"], offset=e["offset"]).copy()
              for e in h["arrays"]}
    return h.get("meta", {}), arrays


def write(path, meta, arrays):
    entries, blobs, off = [], [], 0
    for name, a in arrays.items():
        a = np.ascontiguousarray(a); raw = a.tobytes()
        entries.append({"name": name, "type": NAMES[a.dtype.str if a.dtype.itemsize > 1 else "|u1"], "offset": off, "length": int(a.size)})
        pad = (-len(raw)) % 4; blobs.append(raw + b"\0" * pad); off += len(raw) + pad
    h = json.dumps({"meta": meta, "arrays": entries}, separators=(",", ":")).encode()
    h += b" " * ((-len(h)) % 4)
    body = b"MHP1" + struct.pack("<I", len(h)) + h + b"".join(blobs)
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        f.write(gzip.compress(body, compresslevel=9, mtime=0))


def decode_targets(meta, a):
    """name -> (absolute vertex indices, float deltas)."""
    planes = [a[f"{c}lo"].astype(np.uint16) | (a[f"{c}hi"].astype(np.uint16) << 8) for c in "xyz"]
    out = {}
    for name, (off, n, scale) in meta["targets"].items():
        off, n = int(off), int(n)
        idx = np.cumsum(a["idx"][off:off + n].astype(np.int64))
        d = np.stack([np.add.accumulate(p[off:off + n], dtype=np.uint16).view(np.int16) for p in planes], 1) * scale
        out[name] = (idx, d.astype(np.float64))
    return out


def encode_targets(targets, quantum=0.001):
    """Inverse of decode_targets; per-target scale grows only if a delta would overflow int16."""
    table, idx_all, planes = {}, [], [[], [], [], [], [], []]
    off = 0
    for name, (idx, d) in targets.items():
        order = np.argsort(idx); idx = np.asarray(idx)[order]; d = np.asarray(d)[order]
        scale = max(quantum, float(np.abs(d).max(initial=0)) / 32000.0)
        q = np.round(d / scale).astype(np.int64)
        keep = np.any(q != 0, axis=1); idx, q = idx[keep], q[keep]
        di = np.diff(np.concatenate([[0], idx])).astype(np.uint16)
        dq = np.diff(np.vstack([np.zeros((1, 3), np.int64), q]), axis=0).astype(np.int16).view(np.uint16)
        idx_all.append(di)
        for c in range(3):
            planes[2 * c].append((dq[:, c] & 0xFF).astype(np.uint8)); planes[2 * c + 1].append((dq[:, c] >> 8).astype(np.uint8))
        table[name] = [off, int(len(idx)), scale]; off += len(idx)
    cat = lambda xs: np.concatenate(xs) if xs else np.zeros(0)
    arrays = {"idx": cat(idx_all).astype(np.uint16)}
    for k, c in enumerate(["xlo", "xhi", "ylo", "yhi", "zlo", "zhi"]):
        arrays[c] = cat(planes[k]).astype(np.uint8)
    return {"format": 2, "targets": table}, arrays
