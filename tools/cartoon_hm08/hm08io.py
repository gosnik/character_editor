"""I/O helpers for the cartoon hm08 rework: MakeHuman OBJ/.target files and Meshy GLBs.

Units: MakeHuman decimetres, Y-up, +Z forward.  Meshy GLBs are metres, Y-up,
+Z forward, so source geometry is multiplied by 10 on load.
"""
import json, struct
from pathlib import Path
import numpy as np

CE = Path(__file__).resolve().parents[2]
BASE_OBJ = CE / "assets/makehuman/data/3dobjs/base.obj"
# The macro and breast targets the fits need are copied here (CC0, MakeHuman system assets).
TARGETS = CE / "assets/makehuman/data/targets"
ASSETS = CE / "references"  # Meshy blanks, Superhuman references and style sheets


class Obj:
    """hm08 OBJ kept line-for-line so a reshaped copy differs only in `v` lines."""

    def __init__(self, path=BASE_OBJ):
        self.lines = Path(path).read_text().splitlines()
        v, faces, groups, g = [], [], [], None
        for ln in self.lines:
            if ln.startswith("v "):
                v.append([float(x) for x in ln.split()[1:4]])
            elif ln.startswith("g "):
                g = ln[2:].strip()
            elif ln.startswith("f "):
                faces.append([int(t.split("/")[0]) - 1 for t in ln.split()[1:]])
                groups.append(g)
        self.v = np.array(v)
        self.faces = faces
        self.face_group = groups

    def group_faces(self, pred):
        return [f for f, g in zip(self.faces, self.face_group) if pred(g)]

    def group_verts(self, pred):
        return np.unique(np.concatenate([np.array(f) for f in self.group_faces(pred)]))

    def write(self, path, coords, keep=None):
        """Write reshaped OBJ; `keep(group)` optionally drops face groups (preview only)."""
        out, vi, g = [], 0, None
        for ln in self.lines:
            if ln.startswith("v "):
                x, y, z = coords[vi]; vi += 1
                out.append(f"v {x:.6f} {y:.6f} {z:.6f}")
                continue
            if ln.startswith("g "):
                g = ln[2:].strip()
            if keep and ln.startswith("f ") and not keep(g):
                continue
            out.append(ln)
        Path(path).write_text("\n".join(out) + "\n")


def body_group(g):
    return g == "body"


def read_target(path):
    idx, d = [], []
    for ln in Path(path).read_text().splitlines():
        if not ln or ln[0] == "#":
            continue
        p = ln.split()
        idx.append(int(p[0])); d.append([float(x) for x in p[1:4]])
    return np.array(idx, dtype=np.int64), np.array(d).reshape(-1, 3)


def write_target(path, idx, d, header):
    path = Path(path); path.parent.mkdir(parents=True, exist_ok=True)
    keep = np.abs(d).max(1) > 5e-7
    body = "\n".join(f"{i} {x:.6f} {y:.6f} {z:.6f}" for i, (x, y, z) in zip(idx[keep], d[keep]))
    path.write_text(header + body + "\n")


def apply_targets(coords, weights):
    out = coords.copy()
    for name, w in weights.items():
        if w:
            i, d = read_target(TARGETS / (name + ".target"))
            out[i] += w * d
    return out


def macro_shape(base, gender, muscular=False):
    """MakeHuman default macros (age 25, average muscle/weight, equal ethnic mix), or
    with `muscular` the editor's muscle=1 shape (breastSize=1 for the female)."""
    g = "female" if gender == 0 else "male"
    m = "maxmuscle" if muscular else "averagemuscle"
    w = {f"macrodetails/universal-{g}-young-{m}-averageweight": 1.0}
    if muscular and gender == 0:
        w["breast/female-young-maxmuscle-averageweight-maxcup-averagefirmness"] = 1.0
    for r in ("african", "asian", "caucasian"):
        w[f"macrodetails/{r}-{g}-young"] = 1 / 3
    return apply_targets(base, w)


# --- glTF -------------------------------------------------------------------

def _glb(path):
    b = Path(path).read_bytes(); p = 12; js = blob = None
    while p < len(b):
        n, t = struct.unpack_from("<II", b, p); d = b[p + 8:p + 8 + n]; p += 8 + n
        if t == 0x4E4F534A: js = json.loads(d)
        elif t == 0x004E4942: blob = d
    return js, blob


def _acc(js, blob, i):
    a = js["accessors"][i]; v = js["bufferViews"][a["bufferView"]]
    k = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}[a["type"]]
    dt = np.dtype({5126: "<f4", 5125: "<u4", 5123: "<u2", 5121: "u1"}[a["componentType"]])
    off = v.get("byteOffset", 0) + a.get("byteOffset", 0); st = v.get("byteStride", dt.itemsize * k)
    return np.ndarray((a["count"], k), dt, blob, off, (st, dt.itemsize)).copy()


def glb_mesh(path):
    """Positions (dm) and triangles of all primitives, welded by exact position."""
    js, blob = _glb(path); P, F, off = [], [], 0
    for m in js["meshes"]:
        for q in m["primitives"]:
            v = _acc(js, blob, q["attributes"]["POSITION"]).astype(np.float64)
            ix = _acc(js, blob, q["indices"]).reshape(-1) if "indices" in q else np.arange(len(v))
            P.append(v); F.append(ix.reshape(-1, 3).astype(np.int64) + off); off += len(v)
    P = np.concatenate(P) * 10.0; F = np.concatenate(F)
    _, inv = np.unique(np.round(P, 6), axis=0, return_inverse=True)
    inv = inv.reshape(-1); first = np.zeros(inv.max() + 1, np.int64); first[inv[::-1]] = np.arange(len(inv))[::-1]
    return P[first], inv[F]


def glb_joints(rigged_path, source_points):
    """Meshy rig joint positions in the unrigged source frame (dm)."""
    js, blob = _glb(rigged_path); skin = js["skins"][0]
    ibm = _acc(js, blob, skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1)
    q = js["meshes"][0]["primitives"][0]
    rig_pts = _acc(js, blob, q["attributes"]["POSITION"]).astype(np.float64) * 10
    shift = source_points.min(0)[1] - rig_pts.min(0)[1]
    out = {}
    for j, m in zip(skin["joints"], ibm):
        p = np.linalg.inv(m)[:3, 3] * 10
        out[js["nodes"][j]["name"]] = p + np.array([0, shift, 0])
    return out
