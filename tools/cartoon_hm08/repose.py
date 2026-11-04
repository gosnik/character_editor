"""Re-pose a fitted cartoon shape into hm08's rest pose.

The Meshy blanks stand with straighter elbows, lower arms and closer legs than hm08.
Every clip in the editor is retargeted against hm08's rest pose, so a cartoon offset
that also changed the limb angles would put every animation's arms and legs in the
wrong place. Each limb segment of the cartoon shape is rotated back to the realistic
segment direction (keeping the cartoon segment's length and shape), and the result is
blended with hm08's own skin weights from `assets/native/base.pack`.
"""
import numpy as np

import mhp1
from hm08io import CE

# (bones moved by the segment, start joint, end joint); segments are chained, each
# one starting where its (re-posed) parent ends. Bones not listed stay put.
CHAINS = [
    [
        (("upperarm01", "upperarm02"), "upperarm01", "lowerarm01"),
        (("lowerarm01", "lowerarm02"), "lowerarm01", "wrist"),
        (("wrist", "metacarpal*", "finger*"), "wrist", "finger3-1"),
    ],
    [
        (("upperleg01", "upperleg02"), "upperleg01", "lowerleg01"),
        (("lowerleg01", "lowerleg02"), "lowerleg01", "foot"),
        (("foot", "toe*"), "foot", "toe3-1"),
    ],
]


def rotation_between(a, b):
    a = a / np.linalg.norm(a); b = b / np.linalg.norm(b)
    v = np.cross(a, b); c = float(np.dot(a, b))
    if np.linalg.norm(v) < 1e-9:
        return np.eye(3)
    k = np.array([[0, -v[2], v[1]], [v[2], 0, -v[0]], [-v[1], v[0], 0]])
    return np.eye(3) + k + k @ k / (1 + c)


def repose(real, cartoon):
    meta, arr = mhp1.read(CE / "assets/native/base.pack")
    joints = meta["joints"]; bones = [b["name"] for b in meta["bones"]]
    jp = lambda X, n: X[joints[n + "____head"]].mean(0)
    n = len(real)
    transforms = {}  # bone index -> (R, cartoon pivot, target pivot)
    for side in ("L", "R"):
        for chain in CHAINS:
            R_acc = np.eye(3); target_start = None
            for names, start, end in chain:
                s, e = f"{start}.{side}", f"{end}.{side}"
                c0, c1 = jp(cartoon, s), jp(cartoon, e)
                r0, r1 = jp(real, s), jp(real, e)
                pivot_t = c0 if target_start is None else target_start
                R = rotation_between(R_acc @ (c1 - c0), r1 - r0) @ R_acc
                for i, b in enumerate(bones):
                    stem, _, bs = b.rpartition(".")
                    if bs == side and any(stem == x or (x.endswith("*") and stem.startswith(x[:-1])) for x in names):
                        transforms[i] = (R, c0, pivot_t)
                target_start = pivot_t + R @ (c1 - c0)
                R_acc = R
    off, wb, wv = arr["wOff"], arr["wBone"], arr["wVal"]
    out = cartoon.copy()
    for v in range(n):
        acc = np.zeros(3); total = 0.0
        for k in range(off[v], off[v + 1]):
            w = float(wv[k]); t = transforms.get(int(wb[k]))
            if t is None:
                acc += w * cartoon[v]
            else:
                R, c0, p = t
                acc += w * (R @ (cartoon[v] - c0) + p)
            total += w
        if total > 0:
            out[v] = acc / total
    return out


# Fattening across the hand (cartoon fingers are chunkier than hm08's at equal length).
HAND_THICKEN = 1.18


def transplant_hands(real, reposed):
    """Replace each fitted hand with hm08's own hand, scaled to the cartoon hand.

    hm08's fingers are much thinner than the Meshy blanks' closed, chunky fingers, so
    fitting one to the other fuses them. After `repose` the hand points the realistic
    way, so the realistic hand only needs moving to the cartoon wrist and scaling:
    by the cartoon/realistic hand length along the hand, a little more across it. It is
    blended in by each vertex's hm08 skin weight on the hand bones, so the wrist stays
    continuous."""
    meta, arr = mhp1.read(CE / "assets/native/base.pack")
    joints = meta["joints"]; bones = [b["name"] for b in meta["bones"]]
    off, wb, wv = arr["wOff"], arr["wBone"], arr["wVal"]
    out = reposed.copy()
    for side in ("L", "R"):
        hand = {i for i, b in enumerate(bones) if b.endswith("." + side)
                and b.split(".")[0].startswith(("wrist", "metacarpal", "finger"))}
        jp = lambda X, n: X[joints[f"{n}.{side}____head"]].mean(0)
        tip = lambda X: X[joints[f"finger3-3.{side}____tail"]].mean(0)
        w_r, w_c = jp(real, "wrist"), jp(reposed, "wrist")
        axis = tip(real) - w_r; length_r = np.linalg.norm(axis); axis /= length_r
        s = np.linalg.norm(tip(reposed) - w_c) / length_r
        A = s * (HAND_THICKEN * np.eye(3) + (1 - HAND_THICKEN) * np.outer(axis, axis))
        for v in range(len(real)):
            h = sum(float(wv[k]) for k in range(off[v], off[v + 1]) if int(wb[k]) in hand)
            if h > 0:
                out[v] = h * (w_c + A @ (real[v] - w_r)) + (1 - h) * reposed[v]
    return out
