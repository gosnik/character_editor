#!/usr/bin/env python3
"""Fit hm08 (MakeHuman base mesh) to the Meshy cartoon blanks, keeping hm08 topology.

Pipeline per sex:
  1. hm08 default-macro shape for that sex (MakeHuman targets).
  2. Landmark pairs: hand-picked face/ear/hand picks (landmarks.json), rule-based body
     extremes, and rig joints (Meshy auto-rig <-> hm08 joint helper centroids).
  3. Thin-plate warp of all 19,158 vertices from those pairs (coarse proportions).
  4. Laplacian-regularised non-rigid ICP of the visible body surface onto the Meshy
     surface, with a stiffness schedule.  Eye-socket and mouth-cavity interiors are
     excluded from the data term and follow the regulariser.
  5. Helper geometry (joints, eyes, teeth, tongue, lashes, clothing helpers) follows the
     surface displacement; eyeballs are refit to the cartoon eye openings.
  6. Bilateral symmetrisation through the hm08 mirror map.
"""
import argparse, json, sys
from pathlib import Path
import numpy as np
import scipy.sparse as sp
from scipy.sparse.linalg import factorized
from scipy.spatial import cKDTree

sys.path.insert(0, str(Path(__file__).resolve().parent))
from hm08io import Obj, body_group, macro_shape, glb_mesh, glb_joints, ASSETS, CE
from geom import tri_list, vertex_normals, mirror_map, mirror, load_picks, pick_points, auto_points, TPS, raycast
from features import facial_openings

SOURCES = {0: "Meshy_AI_Blank_Character_Model_F.glb", 1: "Meshy_AI_Blank_Character_Model_M.glb"}
RIGS = {0: "female-rigged.glb", 1: "male-rigged.glb"}
LABEL = {0: "female", 1: "male"}
FINGERS = ["thumb", "index", "middle", "ring", "pinky"]
# The Meshy eyes are fully round; the reference style sheets have almond openings with
# the lids over the top of the iris.  The eyeball keeps the Meshy size; only the opening
# height is reduced (and dropped slightly so the upper lid does most of the covering).
# Per sex (female, male): the male blank's eyes are already narrower.
EYE_OPEN = (0.85, 1.0)
EYE_DROP = 0.18
REVERSE = 1.0  # weight of the reference->hm08 ICP term for the Superhuman fits
EYE_CLEAR = 0.012  # dm between the eyeball and the lid margin
SOCKET_REACH = 3.0  # in opening half-sizes: how far round the eye the socket fill reaches
SOCKET_ITERS = 80
SOCKET_FILL = 0.85  # share of the hollow filled next to the eye
EYE_BAND = 0.15  # dm of surrounding skin that follows the lid margin onto the ball


def group_centroid(o, V, name):
    return V[o.group_verts(lambda g: g == name)].mean(0)


def hm08_landmark_ids(o, ref, picks):
    """Resolve hm08 picks (made on the female default shape) to vertex indices."""
    body = o.group_verts(body_group); T = tri_list(o.group_faces(body_group))
    tree = cKDTree(ref[body]); ids = {}
    for k, p in pick_points(picks["hm08"], ref, T).items():
        ids[k] = int(body[tree.query(p)[1]])
    # fingertips: furthest body vertex along each distal phalanx
    fj = np.array([[group_centroid(o, ref, f"joint-l-finger-{n}-{k}") for k in range(1, 5)] for n in range(1, 6)])
    owner = cKDTree(fj.reshape(-1, 3)).query(ref[body])[1] // 4
    for n, name in enumerate(FINGERS, 1):
        j3, j4 = fj[n - 1, 2], fj[n - 1, 3]
        d = (j4 - j3) / np.linalg.norm(j4 - j3)
        near = body[(owner == n - 1) & (np.linalg.norm(ref[body] - j4, axis=1) < 0.35)]
        ids[f"{name}_tip"] = int(near[np.argmax((ref[near] - j4) @ d)])
    # toe tips: most forward vertex of each toe
    tj, town = [], []
    for n in range(1, 6):
        for k in range(1, 5):
            g = f"joint-l-toe-{n}-{k}"
            if len(o.group_faces(lambda x: x == g)):
                tj.append(group_centroid(o, ref, g)); town.append(n)
    tj = np.array(tj); town = np.array(town)
    owner = town[cKDTree(tj).query(ref[body])[1]]
    for n in range(1, 6):
        near = body[(owner == n) & (np.linalg.norm(ref[body] - tj[town == n][-1], axis=1) < 0.4)]
        ids[f"toe{n}_tip"] = int(near[np.argmax(ref[near, 2])])
    return ids


def landmark_pairs(o, V, sex, src_P, src_T, joints, hm_ids):
    picks = load_picks()
    body = o.group_verts(body_group); Vb = V[body]
    # body-rule landmarks on both meshes
    hm_eye = (V[hm_ids["eye_upper"]][1] + V[hm_ids["eye_lower"]][1]) / 2
    sp_ = pick_points(picks[["meshy_f", "meshy_m"][sex]], src_P, src_T)
    src_eye = (sp_["eye_upper"][1] + sp_["eye_lower"][1]) / 2
    hm_auto = auto_points(np.vstack([Vb]), hm_eye, group_centroid(o, V, "joint-neck")[1],
                          group_centroid(o, V, "joint-pelvis")[1], group_centroid(o, V, "joint-l-ankle")[1])
    src_auto = auto_points(src_P, src_eye, joints["neck"][1], joints["Hips"][1], joints["LeftFoot"][1])
    tree = cKDTree(Vb)
    src, dst, ids, names = [], [], [], []
    def add(name, hv, sp):
        for s in ((1, -1) if abs(sp[0]) > 0.02 else (1,)):
            h = hv if s == 1 else None
            if s == -1:  # mirrored pair
                h = int(o._mirror[hv]); sp = mirror(sp)
            src.append(V[h]); dst.append(sp); ids.append(h); names.append(name + ("" if s == 1 else "_r"))
    for k, hv in hm_ids.items():
        if k in sp_:
            add(k, hv, sp_[k])
    for k, hp in hm_auto.items():
        add(k, int(body[tree.query(hp)[1]]), src_auto[k])
    surface = (np.array(src), np.array(dst), np.array(ids), names)
    # joints: helper centroids (not surface vertices) -> only used by the TPS
    js, jd = [], []
    for mj, hg in picks["joints"].items():
        if mj.startswith("_"):
            continue
        js.append(group_centroid(o, V, hg)); jd.append(joints[mj])
        if "-l-" in hg:
            js.append(group_centroid(o, V, hg.replace("-l-", "-r-"))); jd.append(joints[mj.replace("Left", "Right")])
    return surface, (np.array(js), np.array(jd))


def uniform_laplacian(n, faces):
    rows, cols = [], []
    for f in faces:
        for i in range(len(f)):
            a, b = f[i], f[(i + 1) % len(f)]
            rows += [a, b]; cols += [b, a]
    A = sp.coo_matrix((np.ones(len(rows)), (rows, cols)), shape=(n, n)).tocsr()
    A.data[:] = 1.0
    deg = np.asarray(A.sum(1)).ravel()
    return sp.identity(n, format="csr") - sp.diags(1 / np.maximum(deg, 1)) @ A


def interior_mask(o, V, body):
    """Body vertices lining the eye sockets and mouth cavity (excluded from data term)."""
    ex = np.zeros(len(body), bool)
    for side in "lr":
        ev = o.group_verts(lambda g, s=side: g == f"helper-{s}-eye")
        c = V[ev].mean(0); r = np.linalg.norm(V[ev] - c, axis=1).max()
        ex |= np.linalg.norm(V[body] - c, axis=1) < r * 1.12
    mouth = o.group_verts(lambda g: g in ("helper-upper-teeth", "helper-lower-teeth", "helper-tongue"))
    d, _ = cKDTree(V[mouth]).query(V[body])
    ex |= d < 0.12
    return ex


# Body parts for correspondence: (part, start joint, end joint) on the Meshy rig and the
# hm08 joint helpers. A hanging hand right beside a bulky thigh (the Superhuman female)
# otherwise pulls thigh vertices onto the hand.
PART_SEGMENTS = [
    (0, "Hips", "neck", "joint-pelvis", "joint-neck"), (0, "neck", "Head", "joint-neck", "joint-head"),
    (1, "LeftArm", "LeftForeArm", "joint-l-shoulder", "joint-l-elbow"),
    (1, "LeftForeArm", "LeftHand", "joint-l-elbow", "joint-l-hand"),
    (3, "LeftUpLeg", "LeftLeg", "joint-l-upper-leg", "joint-l-knee"),
    (3, "LeftLeg", "LeftFoot", "joint-l-knee", "joint-l-ankle"),
]
PART_AMBIGUOUS = 1.35  # second-nearest part within this factor: a junction, matches anything


def part_labels(P, segments):
    """Nearest-segment part label per point (hands extend past the wrist, feet forward of
    the ankle), plus a flag for junction points near two parts."""
    best = np.full((len(P), 5), np.inf)
    for part, a, b in segments:
        ab = b - a; t = np.clip(((P - a) @ ab) / (ab @ ab), 0, 1)
        d = np.linalg.norm(P - (a + t[:, None] * ab), axis=1)
        best[:, part] = np.minimum(best[:, part], d)
    order = np.sort(best, 1)
    return best.argmin(1), order[:, 1] < PART_AMBIGUOUS * order[:, 0]


def segments_from(points):
    """PART_SEGMENTS resolved with `points(name)`, mirrored for the right side and extended
    to the hand and toes."""
    out = []
    for part, ma, mb, ha, hb in PART_SEGMENTS:
        a, b = points(ma, ha), points(mb, hb)
        out.append((part, a, b))
        if part == 1:
            out.append((part, b, b + (b - a) * 0.6))
        if part == 3 and ma == "LeftLeg":
            out.append((part, b, b + np.array([0, -0.5, 1.3])))
    for part, a, b in list(out):
        if part:
            out.append((part + 1, a * [-1, 1, 1], b * [-1, 1, 1]))
    return out


def nicp(V0b, faces_b, src_P, src_N, lm_local, lm_dst, exclude, schedule, log, parts=None, reverse=0.0):
    """Non-rigid ICP. With `reverse` > 0 reference points also pull their nearest vertex
    (bidirectional), so the surface swells into bulges the nearest-point term alone
    never reaches (the Superhuman muscles)."""
    n = len(V0b); L = uniform_laplacian(n, faces_b); LtL = (L.T @ L).tocsc()
    T = tri_list(faces_b); tree = cKDTree(src_P)
    X = V0b.copy(); delta = L @ V0b
    rev_ids = np.random.default_rng(3).choice(len(src_P), min(len(src_P), 60000), replace=False)
    if parts is not None:
        (lab_b, amb_b), (lab_s, amb_s) = parts
    for alpha, iters, thr, lw in schedule:
        for _ in range(iters):
            N = vertex_normals(X, T)
            d, j = tree.query(X, k=8)
            # best of 8 nearest by distance + normal disagreement penalty
            cos = np.einsum("nkc,nc->nk", src_N[j], N)
            score = d + 0.5 * thr * (1 - cos)
            if parts is not None:
                ok = (lab_s[j] == lab_b[:, None]) | amb_b[:, None] | amb_s[j]
                score = np.where(ok, score, np.inf)
                cos = np.where(ok, cos, -1.0)
            b = np.argmin(score, 1); jj = j[np.arange(n), b]; dd = d[np.arange(n), b]; cc = cos[np.arange(n), b]
            w = ((dd < thr) & (cc > 0.4) & ~exclude).astype(float)
            # point-to-plane flavoured target: move along the source normal mostly
            q = src_P[jj]; nq = src_N[jj]
            tgt = X + nq * ((q - X) * nq).sum(1, keepdims=True) * 0.7 + (q - X) * 0.3
            if reverse > 0:
                ds, iv = cKDTree(X).query(src_P[rev_ids])
                ok = (ds < thr) & ((src_N[rev_ids] * N[iv]).sum(1) > 0.4) & ~exclude[iv]
                if parts is not None:
                    ok &= (lab_s[rev_ids] == lab_b[iv]) | amb_s[rev_ids] | amb_b[iv]
                acc = np.zeros((n, 3)); cnt = np.zeros(n)
                np.add.at(acc, iv[ok], src_P[rev_ids][ok]); np.add.at(cnt, iv[ok], 1)
                got = cnt > 0
                wr = np.where(got, reverse, 0.0)
                tr = np.where(got[:, None], acc / np.maximum(cnt, 1)[:, None], X)
                tgt = (w[:, None] * tgt + wr[:, None] * tr) / np.maximum(w + wr, 1e-9)[:, None]
                w = w + wr
            W = sp.diags(w)
            Lw = np.zeros(n); Lw[lm_local] = lw
            A = (alpha * LtL + W + sp.diags(Lw)).tocsc()
            rhs = alpha * (L.T @ delta) + w[:, None] * tgt
            rhs[lm_local] += lw * lm_dst
            solve = factorized(A)
            X = np.column_stack([solve(rhs[:, k]) for k in range(3)])
        dist = tree.query(X[~exclude])[0]
        log(f"  alpha={alpha:<6} thr={thr:<4} mean={dist.mean():.4f} p95={np.percentile(dist,95):.4f} max={dist.max():.4f} dm")
    return X


def propagate(V_from, disp_b, body, targets_idx, V_at, k=12):
    """Spread body-surface displacement to helper vertices (Gaussian KNN)."""
    tree = cKDTree(V_from[body]); d, ix = tree.query(V_at[targets_idx], k=k)
    s = np.maximum(d[:, :1], 0.05) * 1.5
    w = np.exp(-(d / s) ** 2); w /= w.sum(1, keepdims=True)
    return (disp_b[ix] * w[:, :, None]).sum(1)


def opening_targets(V, feat, sp, src_P, src_T, eye_open):
    """Rim vertex -> point on the cartoon eye outline / smile line (left side + midline)."""
    ids, pts = [], []
    # eye: hm08 rim parametrised on its own ellipse, mapped onto the ellipse through the picks
    rim = feat["eye_rim"]; R = V[rim]
    c = (R[:, :2].min(0) + R[:, :2].max(0)) / 2; ab = (R[:, :2].max(0) - R[:, :2].min(0)) / 2
    e = (R[:, :2] - c) / ab; ang = np.arctan2(e[:, 1], e[:, 0]); rho = np.linalg.norm(e, axis=1)
    rho = rho / np.median(rho)
    c2 = np.array([(sp["eye_outer"][0] + sp["eye_inner"][0]) / 2, (sp["eye_upper"][1] + sp["eye_lower"][1]) / 2])
    ab2 = np.array([abs(sp["eye_outer"][0] - sp["eye_inner"][0]) / 2, (sp["eye_upper"][1] - sp["eye_lower"][1]) / 2])
    c2 = c2 - [0, EYE_DROP * ab2[1] * (1 - eye_open)]; ab2 = ab2 * [1, eye_open]
    for v, a, r_ in zip(rim, ang, rho):
        # keep each margin vertex's radial offset (lid thickness) relative to the outline
        u = c2 + ab2 * (1 + 0.6 * (r_ - 1)) * [np.cos(a), np.sin(a)]
        ids.append(v); pts.append(raycast(src_P, src_T, "front", *u))
    # mouth: both lip margins onto the smile line (parabola through corners and centre)
    rim = feat["mouth_rim"]; R = V[rim]; xc = np.abs(R[:, 0]).max()
    yc = sp["upper_lip"][1] - 0.012; xc2, ycor = sp["mouth_corner"][0], sp["mouth_corner"][1]
    # hm08 slit line (parabola through its own centre and corners) to measure each margin's offset
    yc0 = R[np.abs(R[:, 0]) < 0.03, 1].mean(); ycor0 = R[np.abs(R[:, 0]) > xc - 0.02, 1].mean()
    for v, p in zip(rim, R):
        t = p[0] / xc; x = t * xc2 * 0.97
        off = p[1] - (yc0 + (ycor0 - yc0) * t * t)
        y = yc + (ycor - yc) * t * t + off * 1.5
        ids.append(v); pts.append(raycast(src_P, src_T, "front", x if abs(x) > 1e-3 else 1e-3, y))
    return np.array(ids), np.array(pts)


def relax(Xb, faces_b, region, curves, tree, src_P, src_N, iters=8, lam=0.5):
    """Smooth a band (and its rim curves) then re-project onto the cartoon surface."""
    n = len(Xb); adj = [set() for _ in range(n)]
    for f in faces_b:
        for i in range(len(f)):
            adj[f[i]].add(f[(i + 1) % len(f)]); adj[f[(i + 1) % len(f)]].add(f[i])
    region = np.asarray(sorted(set(region)))
    cset = set(int(c) for c in np.concatenate(curves)) if curves else set()
    cnb = {c: [m for m in adj[c] if m in cset] for c in cset}
    for _ in range(iters):
        Y = Xb.copy()
        for v in region:
            ns = cnb.get(v) if v in cset else list(adj[v])
            if ns:
                Y[v] = Xb[v] + lam * (Xb[ns].mean(0) - Xb[v])
        d, j = tree.query(Y[region])
        Y[region] -= src_N[j] * ((Y[region] - src_P[j]) * src_N[j]).sum(1, keepdims=True)
        Xb = Y
    return Xb


def rings(faces_b, seeds, k, n):
    adj = [set() for _ in range(n)]
    for f in faces_b:
        for i in range(len(f)):
            adj[f[i]].add(f[(i + 1) % len(f)]); adj[f[(i + 1) % len(f)]].add(f[i])
    cur = set(int(s) for s in seeds)
    for _ in range(k):
        cur |= set(m for v in cur for m in adj[v])
    return np.array(sorted(cur))


def harmonic_fill(n, faces, region, values, known):
    """Solve uniform-Laplace for `region` rows given fixed values at `known` rows."""
    L = uniform_laplacian(n, faces).tocsr()
    Lrr = L[region][:, region].tocsc(); Lrk = L[region][:, known]
    solve = factorized(Lrr)
    rhs = -(Lrk @ values[known])
    out = values.copy(); out[region] = np.column_stack([solve(rhs[:, k]) for k in range(3)])
    return out


def fit_sphere(P):
    A = np.hstack([2 * P, np.ones((len(P), 1))]); b = (P ** 2).sum(1)
    x = np.linalg.lstsq(A, b, rcond=None)[0]; c = x[:3]
    return c, float(np.sqrt(x[3] + c @ c))


def fit(sex, log=print, source=None, muscular=False):
    """Fit hm08 to a cartoon surface. `source` (default the Meshy blank) is a GLB in
    references/ in the blank's frame and pose; `muscular` starts from MakeHuman's
    max-muscle shape (and max cup for the female), for the Superhuman references."""
    o = Obj(); o._mirror = mirror_map(o.v); mir = o._mirror
    ref_f = macro_shape(o.v, 0)
    V = macro_shape(o.v, sex, muscular)
    picks = load_picks()
    hm_ids = hm08_landmark_ids(o, ref_f, picks)
    feat = facial_openings(o, ref_f)
    src_P, src_F = glb_mesh(ASSETS / (source or SOURCES[sex]))
    import geom
    geom.PICK_FALLBACK = glb_mesh(ASSETS / SOURCES[sex]) if source else None
    src_N = vertex_normals(src_P, src_F)
    joints = glb_joints(CE / "assets/native/cartoon_meshy/rigging" / RIGS[sex], src_P)
    (ls, ld, lid, names), (js, jd) = landmark_pairs(o, V, sex, src_P, src_F, joints, hm_ids)
    sp_ = pick_points(picks[["meshy_f", "meshy_m"][sex]], src_P, src_F)
    rid, rpt = opening_targets(V, feat, sp_, src_P, src_F, EYE_OPEN[sex])
    # right side: mirrored ids and points (skip exact midline duplicates)
    side = np.abs(V[rid, 0]) > 1e-4
    rid = np.concatenate([rid, mir[rid[side]]]); rpt = np.vstack([rpt, rpt[side] * [-1, 1, 1]])
    log(f"{LABEL[sex]}: {len(ls)} surface landmarks, {len(js)} joint landmarks, {len(rid)} rim targets")
    warp = TPS(np.vstack([ls, js, V[rid[::3]]]), np.vstack([ld, jd, rpt[::3]]), reg=0.02)
    W0 = warp(V)
    body = o.group_verts(body_group); local = -np.ones(len(V), int); local[body] = np.arange(len(body))
    faces_b = [[local[v] for v in f] for f in o.group_faces(body_group)]
    eye_in = np.concatenate([feat["eye_inner"], mir[feat["eye_inner"]]])
    interior = np.concatenate([eye_in, feat["mouth_inner"]])
    exclude = np.zeros(len(body), bool); exclude[local[interior]] = True; exclude[local[rid]] = True
    log(f"  excluded verts: {exclude.sum()}")
    # soft constraints: landmarks (weight 1) and rim targets (weight 1)
    cid = np.concatenate([local[lid], local[rid]]); cpt = np.vstack([ld, rpt])
    sched = [(200, 3, 1.0, 5.0), (60, 3, 0.6, 5.0), (20, 3, 0.4, 5.0), (8, 3, 0.3, 4.0), (3, 3, 0.25, 3.0), (1.2, 4, 0.2, 2.0), (0.5, 3, 0.15, 1.5), (0.25, 3, 0.1, 1.0)]
    # Part labels: the hm08 side follows the TPS-warped shape the ICP starts from.
    side = lambda mj, hg: group_centroid(o, W0, hg)
    src_side = lambda mj, hg: joints[mj]
    parts = (part_labels(W0[body], segments_from(side)), part_labels(src_P, segments_from(src_side)))
    Xb = nicp(W0[body], faces_b, src_P, src_N, cid, cpt, exclude, sched, log, parts,
              reverse=REVERSE if source else 0.0)
    # tidy the stretched lid/lip margins and any locally folded faces
    tree = cKDTree(src_P)
    ex_set = set(np.where(exclude)[0]) - set(local[rid])
    eye_rims = [local[feat["eye_rim"]], local[mir[feat["eye_rim"]]]]
    band = rings(faces_b, np.concatenate(eye_rims + [local[feat["mouth_rim"]]]), 3, len(body))
    Q = np.array([f for f in faces_b if len(f) == 4]); Vb0 = V[body]
    def qn(P): return np.cross(P[Q[:, 2]] - P[Q[:, 0]], P[Q[:, 3]] - P[Q[:, 1]])
    flipped = Q[(qn(Vb0) * qn(Xb)).sum(1) < 0].ravel()
    band = np.union1d(band, rings(faces_b, flipped, 1, len(body)))
    band = np.union1d(band, rings(faces_b, local[[hm_ids["ala"], mir[hm_ids["ala"]]]], 3, len(body)))
    band = np.array([v for v in band if v not in ex_set])
    Xb = relax(Xb, faces_b, band, eye_rims + [local[feat["mouth_rim"]]], tree, src_P, src_N)
    log(f"  relaxed {len(band)} verts ({len(np.unique(flipped))} on flipped faces)")
    X = W0.copy(); X[body] = Xb
    # --- interiors: Poisson transplant of hm08's own (scaled) socket/cavity shape,
    # hanging from the fitted rims.  Helpers then follow by similarity / KNN.
    Lb = uniform_laplacian(len(body), faces_b).tocsr(); Vb = V[body]
    def transplant(region, scale):
        known = np.setdiff1d(np.arange(len(body)), region)
        A = Lb[region][:, region].tocsc(); solve = factorized(A)
        rhs = scale * (Lb @ Vb)[region] - Lb[region][:, known] @ X[body][known]
        Xr = np.column_stack([solve(rhs[:, k]) for k in range(3)])
        X[body[region]] = Xr
    hv = lambda k: hm_ids[k]
    s_head = np.linalg.norm(X[hv("ear_top")] - X[hv("chin")]) / np.linalg.norm(V[hv("ear_top")] - V[hv("chin")])
    mouth_r = local[feat["mouth_inner"]]
    transplant(mouth_r, s_head)
    # mouth helpers: similarity that best maps the cavity
    P0 = V[feat["mouth_inner"]]; P1 = X[feat["mouth_inner"]]
    m0, m1 = P0.mean(0), P1.mean(0); U, S_, Wt = np.linalg.svd((P0 - m0).T @ (P1 - m1))
    Rm = (U @ Wt).T
    if np.linalg.det(Rm) < 0: Rm = (U @ np.diag([1, 1, -1]) @ Wt).T
    sm = np.sqrt(((P1 - m1) ** 2).sum() / ((P0 - m0) ** 2).sum())
    mouth_helpers = o.group_verts(lambda g: g in ("helper-upper-teeth", "helper-lower-teeth", "helper-tongue")
                                  or g.startswith("joint-tongue") or g in ("joint-mouth", "joint-jaw"))
    X[mouth_helpers] = m1 + sm * (V[mouth_helpers] - m0) @ Rm.T
    log(f"  head scale {s_head:.2f}, mouth cavity similarity scale {sm:.2f}")
    helpers = np.setdiff1d(np.arange(len(V)), np.concatenate([body, mouth_helpers]))
    X[helpers] = W0[helpers] + propagate(W0, X[body] - W0[body], body, helpers, W0)
    # joint helpers sit inside limbs: carry each through the local affine map of the
    # surrounding skin so it stays centred in the cartoon limb cross-section
    skin = body[~exclude]; tree_v = cKDTree(V[skin])
    joints_ = o.group_verts(lambda g: g.startswith("joint-") and g not in ("joint-mouth", "joint-jaw")
                            and not g.startswith("joint-tongue") and "eye" not in g)
    for g in sorted(set(o.face_group)):
        if not g or not g.startswith("joint-") or g in ("joint-mouth", "joint-jaw") or g.startswith("joint-tongue") or "eye" in g:
            continue
        gv = o.group_verts(lambda x, g=g: x == g); c0 = V[gv].mean(0)
        _, nn = tree_v.query(c0, k=48); P0 = V[skin[nn]]; P1 = X[skin[nn]]
        m0, m1 = P0.mean(0), P1.mean(0)
        A = np.linalg.lstsq(P0 - m0, P1 - m1, rcond=None)[0]
        X[gv] = m1 + (V[gv] - m0) @ A
    # --- shallow sockets: the reference sheets have near-flush eyes, shallower than the
    # Meshy blank. Hollows around each eye are raised towards a smooth membrane spanning
    # brow, cheek and nose bridge (z-only Laplacian with the outer ring held), fading out
    # with distance from the opening. The lid margins ride along.
    rim_ids = feat["eye_rim"]
    oc = (X[rim_ids, :2].min(0) + X[rim_ids, :2].max(0)) / 2
    oh = (X[rim_ids, :2].max(0) - X[rim_ids, :2].min(0)) / 2
    q = np.linalg.norm((X[body, :2] - oc) / oh, axis=1)
    front = X[body, 2] > X[rim_ids, 2].min() - 0.3
    region = body[(q < SOCKET_REACH) & front & (X[body, 0] > 0) & ~np.isin(body, eye_in)]
    rq = q[np.searchsorted(body, region)]
    nbr = [[] for _ in range(len(X))]
    for f in o.group_faces(body_group):
        for i in range(len(f)):
            a, b = f[i], f[(i + 1) % len(f)]
            nbr[a].append(b); nbr[b].append(a)
    z = X[:, 2].copy()
    free = region[rq < SOCKET_REACH - 0.25]
    for _ in range(SOCKET_ITERS):
        z[free] = [np.mean(z[nbr[v]]) for v in free]  # neighbours outside stay fixed
    w = np.clip((SOCKET_REACH - rq) / (SOCKET_REACH - 1.4), 0, 1) ** 2 * SOCKET_FILL
    raise_ = np.maximum(z[region] - X[region, 2], 0) * w
    for side in (region, mir[region]):
        X[side, 2] += raise_
    log(f"  sockets raised: max {raise_.max():.3f} dm, mean {raise_[raise_ > 0].mean():.3f} dm")
    # --- eyeballs: rigid sphere at the cartoon eye (centre behind the opening, depth fitted)
    c2 = np.array([(sp_["eye_outer"][0] + sp_["eye_inner"][0]) / 2, (sp_["eye_upper"][1] + sp_["eye_lower"][1]) / 2])
    ab2 = np.array([abs(sp_["eye_outer"][0] - sp_["eye_inner"][0]) / 2, (sp_["eye_upper"][1] - sp_["eye_lower"][1]) / 2])
    m = (np.linalg.norm((src_P[:, :2] - c2) / ab2, axis=1) < 0.75) & (src_N[:, 2] > 0.3) & (src_P[:, 2] > 0)
    ev = o.group_verts(lambda g: g == "helper-l-eye"); hc = V[ev].mean(0); hr = np.linalg.norm(V[ev] - hc, axis=1).max()
    a_hm = (V[feat["eye_rim"], 0].max() - V[feat["eye_rim"], 0].min()) / 2
    er = 1.15 * hr / a_hm * ab2[0]
    # The ball sits on the Meshy eye surface, centred behind the opening so the iris
    # looks straight out. The lid margin is then laid onto the ball (EYE_CLEAR outside
    # it, by moving along depth) with a fading band of skin around it, and skin the
    # ball would poke through is pushed out over it, so the lids wrap the eye.
    # Pushing the ball behind every lid point instead left a cavity.
    rim = feat["eye_rim"]
    centre_xy = (X[rim, :2].min(0) + X[rim, :2].max(0)) / 2
    ec = np.array([*centre_xy, src_P[m, 2].mean() - er])
    for _ in range(50):
        d = src_P[m] - ec; ec = (src_P[m] - er * d / np.linalg.norm(d, axis=1, keepdims=True)).mean(0); ec[:2] = centre_xy
    # With raised sockets the lids may sit in front of the Meshy eye: bring the ball up
    # so the rim rests on it on average.
    dxy2 = ((X[rim, :2] - ec[:2]) ** 2).sum(1)
    seat = (X[rim, 2] - np.sqrt(np.maximum((er + EYE_CLEAR) ** 2 - dxy2, 0))).mean()
    ec[2] = max(ec[2], seat)
    dxy2 = ((X[rim, :2] - ec[:2]) ** 2).sum(1)
    shift = np.where(dxy2 < (er + EYE_CLEAR) ** 2,
                     ec[2] + np.sqrt(np.maximum((er + EYE_CLEAR) ** 2 - dxy2, 0)) - X[rim, 2], 0.0)
    band = body[(X[body, 0] > 0) & (X[body, 2] > ec[2]) & ~exclude]  # front of the face only
    band = band[~np.isin(band, rim)]
    dist, near = cKDTree(X[rim]).query(X[band])
    fall = np.clip(1 - dist / EYE_BAND, 0, 1) ** 2
    lift = shift[near] * fall
    # Skin around the opening that the ball would poke through is pushed forward over
    # it (with a smooth falloff), as Meshy's lids wrap its bulging eyes.
    z = X[band, 2] + lift
    dxy2 = ((X[band, :2] - ec[:2]) ** 2).sum(1)
    cover = ec[2] + np.sqrt(np.maximum((er + EYE_CLEAR) ** 2 - dxy2, 0))
    push = np.where(dxy2 < (er + EYE_CLEAR) ** 2, np.maximum(cover - z, 0), 0)
    hits = np.nonzero(push > 0)[0]
    if len(hits):
        d, j = cKDTree(X[band[hits]]).query(X[band], k=min(16, len(hits)), distance_upper_bound=EYE_BAND)
        d = np.atleast_2d(d.T).T; j = np.atleast_2d(j.T).T
        ok = np.isfinite(d)
        spread = np.where(ok, push[hits][np.minimum(j, len(hits) - 1)] * np.clip(1 - d / EYE_BAND, 0, 1) ** 2, 0).max(1)
        lift += np.maximum(push, spread)
    for side in (lambda i: i, lambda i: mir[i]):  # both eyes: symmetrisation averages them
        X[side(band), 2] += lift
        X[side(rim), 2] += shift
    # Anything still inside the ball (temple and outer-corner skin beside it) is moved
    # out radially; distance in 3D, so only skin next to the eye is touched.
    near_ball = body[(X[body, 0] > 0) & ~exclude]
    near_ball = near_ball[~np.isin(near_ball, rim)]
    d = X[near_ball] - ec; dist = np.linalg.norm(d, axis=1)
    inside = dist < er + EYE_CLEAR
    out = d[inside] / dist[inside, None] * (er + EYE_CLEAR - dist[inside, None])
    for ids, sign in ((near_ball[inside], 1), (mir[near_ball[inside]], -1)):
        X[ids] += out * [sign, 1, 1]
    log(f"  cartoon eyeball c={ec.round(3)} r={er:.3f} (hm08 r={hr:.3f})")
    eye_rigid = o.group_verts(lambda g: g in ("helper-l-eye", "joint-l-eye", "joint-l-eye-target"))
    X[eye_rigid] = ec + (V[eye_rigid] - hc) * (er / hr)
    X[mir[eye_rigid]] = X[eye_rigid] * [-1, 1, 1]
    for ids in (feat["eye_inner"], mir[feat["eye_inner"]]):
        transplant(local[ids], er / hr)
    # --- bilateral symmetry
    X = 0.5 * (X + X[mir] * [-1, 1, 1])
    vis = body[~exclude]; dist = cKDTree(src_P).query(X[vis])[0]
    flips = float(np.mean((qn(V[body]) * qn(X[body])).sum(1) < 0))
    metrics = dict(surface_mean_dm=float(dist.mean()), surface_p95_dm=float(np.percentile(dist, 95)),
                   surface_max_dm=float(dist.max()), flipped_quad_fraction=flips, height_dm=float(np.ptp(X[body, 1])))
    log(f"  final: mean {dist.mean():.4f} p95 {np.percentile(dist,95):.4f} max {dist.max():.4f} dm, flipped quads {flips:.4%}")
    return o, V, W0, X, dict(landmarks=dict(zip(names, ld.tolist())), hm_ids=hm_ids,
                             eyeball=dict(centre=ec.tolist(), radius=er), metrics=metrics)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(); ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--sex", type=int, default=0); a = ap.parse_args()
    o, V, W0, X, info = fit(a.sex)
    a.out.mkdir(parents=True, exist_ok=True)
    o.write(a.out / f"tps_{LABEL[a.sex]}.obj", W0, keep=body_group)
    o.write(a.out / f"fit_{LABEL[a.sex]}.obj", X, keep=body_group)
    np.save(a.out / f"fit_{LABEL[a.sex]}.npy", X)
    json.dump(info, open(a.out / f"info_{LABEL[a.sex]}.json", "w"), indent=1)
