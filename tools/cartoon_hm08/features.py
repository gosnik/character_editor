"""hm08 facial openings found by front-view visibility on the default female shape.

The eye socket and mouth cavity are closed pockets in hm08.  A body vertex is
"hidden" if a front ray at its (x, y) meets the eyeball/teeth/face first; the rim is
the set of visible vertices with a hidden neighbour.  Hidden vertices form the interior.
"""
from collections import defaultdict
import numpy as np
from geom import tri_list, raycast
from hm08io import body_group


def _neighbours(o):
    nb = defaultdict(set)
    for f in o.group_faces(body_group):
        for i in range(len(f)):
            a, b = f[i], f[(i + 1) % len(f)]
            nb[a].add(b); nb[b].add(a)
    return nb


def _opening(o, V, occluders, centre, radius, nb, tol, zmin):
    body = o.group_verts(body_group)
    T = tri_list(o.group_faces(lambda g: g == "body" or g in occluders))
    cand = body[(np.linalg.norm((V[body] - centre)[:, :2] / radius, axis=1) < 1.0) & (V[body, 2] > zmin)]
    hidden = {}
    for b in cand:
        try:
            hidden[b] = raycast(V, T, "front", V[b, 0], V[b, 1])[2] - V[b, 2] > tol
        except ValueError:
            hidden[b] = False
    rim = np.array([b for b in cand if not hidden[b] and any(hidden.get(n, False) for n in nb[b])])
    inner = np.array([b for b in cand if hidden[b]])
    return rim, inner


def mouth_y(o, V):
    """Lip-contact height: lowest point of the upper incisors."""
    t = o.group_verts(lambda g: g == "helper-upper-teeth")
    front = t[np.abs(V[t, 0]) < 0.04]
    return float(V[front, 1].min())


def _flood(seeds, rim, nb, limit):
    """Grow the pocket through mesh edges without crossing its rim."""
    rim = set(int(r) for r in rim); seen = set(int(s) for s in seeds) - rim; todo = list(seen)
    while todo:
        u = todo.pop()
        for n in nb[u]:
            if n not in seen and n not in rim:
                seen.add(n); todo.append(n)
        if len(seen) > limit:
            raise RuntimeError("pocket flood escaped its rim")
    return np.array(sorted(seen))


def facial_openings(o, V):
    nb = _neighbours(o)
    ev = o.group_verts(lambda g: g == "helper-l-eye")
    c = V[ev].mean(0); r = np.linalg.norm(V[ev] - c, axis=1).max()
    eye_rim, eye_seed = _opening(o, V, ("helper-l-eye",), c, np.array([1.6 * r, 1.6 * r]), nb, 0.004, c[2] - 1.2 * r)
    eye_in = _flood(eye_seed, eye_rim, nb, 2000)
    mouth = o.group_verts(lambda g: g in ("helper-upper-teeth", "helper-lower-teeth", "helper-tongue"))
    mrim, m_seed = _opening(o, V, ("helper-upper-teeth", "helper-lower-teeth", "helper-tongue"),
                            np.array([0.0, mouth_y(o, V), 0]), np.array([0.27, 0.06]), nb, 0.002,
                            V[mouth, 2].min() - 0.05)
    m_in = _flood(m_seed, mrim, nb, 4000)
    return dict(eye_rim=eye_rim, eye_inner=eye_in, eye_centre=c, eye_radius=r,
                mouth_rim=mrim, mouth_inner=m_in)
