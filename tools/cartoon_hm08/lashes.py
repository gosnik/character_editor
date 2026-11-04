"""Cartoon eyelashes: a solid, tapered ribbon along each upper lid margin.

MakeHuman's strand-card lashes break into ragged clumps at toy scale and float off the
enlarged cartoon lids. The Meshy cast and the reference sheets instead have one bold
painted lash line on the upper lid, thickening towards the outer corner and ending in
a flick. This builds that line as a proxy: every ribbon vertex is bound to one upper
rim vertex of hm08 (weight 1) plus a small offset, so it follows the lid through every
morph, the cartoon offsets and blinks exactly.
"""
import numpy as np

import mhp1
from hm08io import CE

# Per sex: base width and outer-corner width (dm), flick length (dm) and its rise.
STYLE = {"female": (0.03, 0.075, 0.1, 0.55), "male": (0.018, 0.035, 0.03, 0.3)}
LIFT = 0.018  # dm in front of the lid margin, clear of the lid skin
OVERLAP = 0.35  # share of the width that hangs over the eye opening (reads as liner)
CURL = 0.25  # forward tilt of the lash direction (0 = in the face plane)
MATERIAL = {"color": "#1d1714", "roughness": 0.6, "metalness": 0, "alpha": "opaque", "doubleSided": True}


def upper_margin(C, rim):
    """Upper-lid rim vertex ids, inner (nose) corner to outer corner."""
    P = C[rim]
    inner, outer = P[:, 0].argmin(), P[:, 0].argmax()
    a, b = P[inner, :2], P[outer, :2]
    t = np.clip(((P[:, :2] - a) @ (b - a)) / ((b - a) @ (b - a)), 0, 1)
    above = P[:, 1] > a[1] + t * (b[1] - a[1]) - 1e-6
    keep = np.nonzero(above)[0]
    keep = keep[np.argsort(t[keep])]
    return rim[keep], t[keep]


def ribbon(C, ids, t, centre, style):
    """Inner and outer edge points of one ribbon, plus the flick past the outer corner."""
    w0, w1, flick, rise = style
    base, tips, refs = [], [], []
    for v, s in zip(ids, t):
        p = C[v]
        d = p[:2] - centre[:2]; d = d / np.linalg.norm(d)
        out = np.array([d[0], d[1], 0.0]) * (1 - CURL) + np.array([0, 0, 1.0]) * CURL
        out /= np.linalg.norm(out)
        w = w0 + (w1 - w0) * s ** 1.5
        root = p + np.array([0, 0, LIFT]) - out * w * OVERLAP
        base.append(root); tips.append(root + out * w); refs.append(v)
    # Flick: continue past the outer corner, rising and narrowing to a point.
    end, prev = C[ids[-1]], C[ids[-2]]
    along = end - prev; along[2] = 0; along /= np.linalg.norm(along)
    up = np.array([0, 1.0, 0])
    for k, f in enumerate((0.35, 0.7, 1.0)):
        p = end + (along * (1 - rise) + up * rise) * flick * f + np.array([0, 0, LIFT])
        w = w1 * (1 - f) + 0.002
        base.append(p); tips.append(p + up * w); refs.append(ids[-1])
    return np.array(base), np.array(tips), np.array(refs)


def write_lashes(o, C, rim, mir, sex, path):
    lash = mhp1.read(CE / "assets/native/proxies/eyelashes_eyelashes01.pack")[0]
    sc = np.array([abs(C[va, d] - C[vb, d]) / den for d, (va, vb, den) in enumerate(lash["scale"])])
    ev = o.group_verts(lambda g: g == "helper-l-eye"); centre = C[ev].mean(0)
    ids, t = upper_margin(C, rim)
    base, tips, refs = ribbon(C, ids, t, centre, STYLE[sex])
    n = len(base)
    P = np.concatenate([base, tips]); R = np.concatenate([refs, refs])
    quads = [[i, i + 1, n + i + 1, n + i] for i in range(n - 1)]
    # Right eye: the mirror image, bound to the mirrored rim vertices.
    P = np.concatenate([P, P * [-1, 1, 1]]); R = np.concatenate([R, mir[R]])
    m = 2 * n
    quads += [[q[0] + m, q[3] + m, q[2] + m, q[1] + m] for q in quads]  # flipped winding
    F = np.array([[q[0], q[1], q[2], q[0], q[2], q[3]] for q in quads]).reshape(-1, 3)
    offsets = (P - C[R]) / sc
    refs3 = np.repeat(R[:, None], 3, 1)
    weights = np.tile([1.0, 0.0, 0.0], (len(P), 1))
    meta = {"id": f"cartoon_eyelashes_{sex}", "kind": "eyelashes", "zDepth": 5, "scale": lash["scale"],
            "material": dict(MATERIAL, name=f"cartoon_eyelashes_{sex}"), "vertexCount": int(len(P))}
    mhp1.write(path, meta, {
        "refs": refs3.reshape(-1).astype(np.uint32), "weights": weights.reshape(-1).astype(np.float32),
        "offsets": offsets.reshape(-1).astype(np.float32),
        "faceSizes": np.full(len(F), 3, np.uint8), "faceVerts": F.reshape(-1).astype(np.uint32),
        "faceUVs": np.zeros(F.size, np.uint32), "uvs": np.array([0.5, 0.5], np.float32),
        "deleteVerts": np.zeros(0, np.uint32),
    })
    return len(ids), len(F)
