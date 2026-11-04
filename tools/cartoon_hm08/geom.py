"""Geometry utilities: ray picking, landmarks, TPS warp, mirror map, normals."""
import json
from pathlib import Path
import numpy as np
from scipy.spatial import cKDTree

HERE = Path(__file__).resolve().parent

# view -> (u axis, v axis, ray axis, ray sign): the hit kept is the one first met by a ray
# travelling in `sign` direction along `axis` (i.e. the extreme surface facing the viewer).
VIEWS = {
    "front": (lambda u, v: (u, v, None), 2, +1),   # viewer at +Z: keep max z
    "side": (lambda u, v: (None, v, -u), 0, +1),   # viewer at +X: keep max x
    "back": (lambda u, v: (-u, v, None), 2, -1),   # viewer at -Z: keep min z
    "bottom": (lambda u, v: (u, None, v), 1, -1),  # viewer at -Y: keep min y
}


def tri_list(faces):
    t = []
    for f in faces:
        for k in range(1, len(f) - 1):
            t.append((f[0], f[k], f[k + 1]))
    return np.array(t, dtype=np.int64)


def raycast(P, T, view, u, v):
    fill, axis, sign = VIEWS[view]
    pt = list(fill(u, v)); other = [i for i in range(3) if i != axis]
    a, b, c = P[T[:, 0]], P[T[:, 1]], P[T[:, 2]]
    q = np.array([pt[i] for i in other])
    A, B, C = a[:, other], b[:, other], c[:, other]
    lo = np.minimum(np.minimum(A, B), C); hi = np.maximum(np.maximum(A, B), C)
    m = np.all((lo <= q) & (q <= hi), axis=1)
    A, B, C, a, b, c = A[m], B[m], C[m], a[m], b[m], c[m]
    v0, v1, w = B - A, C - A, q - A
    den = v0[:, 0] * v1[:, 1] - v0[:, 1] * v1[:, 0]
    den = np.where(np.abs(den) < 1e-15, 1e-15, den)
    s = (w[:, 0] * v1[:, 1] - w[:, 1] * v1[:, 0]) / den
    t = (v0[:, 0] * w[:, 1] - v0[:, 1] * w[:, 0]) / den
    ok = (s >= 0) & (t >= 0) & (s + t <= 1)
    if not ok.any():
        raise ValueError(f"ray {view} ({u},{v}) missed the mesh")
    hits = a[ok] + s[ok, None] * (b[ok] - a[ok]) + t[ok, None] * (c[ok] - a[ok])
    return hits[np.argmax(hits[:, axis] * sign)]


def vertex_normals(P, T):
    n = np.zeros_like(P)
    fn = np.cross(P[T[:, 1]] - P[T[:, 0]], P[T[:, 2]] - P[T[:, 0]])
    for k in range(3):
        np.add.at(n, T[:, k], fn)
    return n / (np.linalg.norm(n, axis=1, keepdims=True) + 1e-12)


def mirror_map(V):
    """Index of each vertex's X-mirror on the symmetric hm08 base."""
    d, ix = cKDTree(V).query(V * [-1, 1, 1])
    assert d.max() < 1e-3, f"base not symmetric (max {d.max()})"
    return ix


def mirror(pt):
    return np.asarray(pt) * [-1, 1, 1]


def load_picks():
    return json.loads((HERE / "landmarks.json").read_text())


def pick_points(spec, P, T):
    return {k: raycast(P, T, *v) for k, v in spec.items() if not k.startswith("_")}


def auto_points(P, eye_y, neck_y, hip_y, ankle_y):
    """Rule-based landmarks applied identically to every body surface (left side)."""
    out = {}
    out["crown"] = P[np.argmax(P[:, 1])]
    head = P[(np.abs(P[:, 0]) < 0.04) & (np.abs(P[:, 1] - eye_y - 0.3) < 0.05)]
    out["back_head"] = head[np.argmin(head[:, 2])]
    out["forehead"] = head[np.argmax(head[:, 2])]
    foot = P[(P[:, 0] > 0.3) & (P[:, 1] < ankle_y)]
    out["heel"] = foot[np.argmin(foot[:, 2])]
    out["toe"] = foot[np.argmax(foot[:, 2])]
    mid = P[(np.abs(P[:, 0]) < 0.06) & (P[:, 1] > hip_y - 3) & (P[:, 1] < hip_y + 1)]
    # crotch: lowest midline point that still belongs to the trunk
    out["crotch"] = mid[np.argmin(mid[:, 1])]
    chest = P[(P[:, 0] > 0.25) & (P[:, 0] < 1.6) & (P[:, 1] > neck_y - 3.2) & (P[:, 1] < neck_y - 0.8)]
    out["chest"] = chest[np.argmax(chest[:, 2])]
    back = P[(np.abs(P[:, 0]) < 0.06) & (P[:, 1] > neck_y - 2.2) & (P[:, 1] < neck_y - 1.2)]
    out["upper_back"] = back[np.argmin(back[:, 2])]
    butt = P[(P[:, 0] > 0.3) & (P[:, 0] < 1.5) & (P[:, 1] > hip_y - 2.0) & (P[:, 1] < hip_y + 0.5)]
    out["buttock"] = butt[np.argmin(butt[:, 2])]
    belly = P[(np.abs(P[:, 0]) < 0.06) & (P[:, 1] > hip_y + 0.3) & (P[:, 1] < neck_y - 2.5)]
    out["belly"] = belly[np.argmax(belly[:, 2])]
    return out


class TPS:
    """3D biharmonic (phi=r) thin-plate warp with affine part and light regularisation."""

    def __init__(self, src, dst, reg=1e-3):
        n = len(src); K = np.linalg.norm(src[:, None] - src[None], axis=2) + reg * np.eye(n)
        Pm = np.hstack([np.ones((n, 1)), src])
        A = np.zeros((n + 4, n + 4)); A[:n, :n] = K; A[:n, n:] = Pm; A[n:, :n] = Pm.T
        b = np.zeros((n + 4, 3)); b[:n] = dst
        self.src = src; self.w = np.linalg.solve(A, b)

    def __call__(self, X):
        out = np.hstack([np.ones((len(X), 1)), X]) @ self.w[len(self.src):]
        for i in range(0, len(X), 4096):
            K = np.linalg.norm(X[i:i + 4096, None] - self.src[None], axis=2)
            out[i:i + 4096] += K @ self.w[:len(self.src)]
        return out
