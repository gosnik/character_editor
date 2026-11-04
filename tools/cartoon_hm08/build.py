#!/usr/bin/env python3
"""Build the cartoon hm08 shapes, the cartoon offset targets and the editor pack.

    python3 tools/cartoon_hm08/build.py [--out assets/cartoon_hm08]

The cartoon look is one more morph on top of MakeHuman's own targets, not a replacement
for them: gender, age, weight, muscle, breasts and every detail target keep their
authored behaviour and range, and a single Cartoon amount adds

    amount * ((1 - gender) * cartoon-female + gender * cartoon-male)

where cartoon-female/-male are the fitted cartoon shapes minus the default-macro
realistic shapes.  Each is split into head, body and chest parts: children keep the full
cartoon head, a lighter body offset and no adult chest, all blending in from age 12
to 18.  The editor scales them by the body's height relative to the adult
default so children and short characters receive proportionate offsets.

Expression units get an additive correction (retargeted unit minus realistic unit) so
blinks and smiles span the larger cartoon eyes and mouth; it is weighted by the same
amount.

Outputs (hm08 topology, MakeHuman decimetres, Y-up):
  <out>/base_female.obj, base_male.obj   full-strength cartoon shapes (all groups/UVs)
  <out>/targets/cartoon/cartoon-{female,male}.target
  <out>/targets/cartoon/expression/**    expression corrections (.target)
  <out>/report.json                      fit metrics and provenance
  assets/native/cartoon_hm08/cartoon.pack  the same offsets and corrections for the editor
"""
import argparse, hashlib, json, shutil, sys, time
from pathlib import Path
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mhp1
from hm08io import Obj, ASSETS, CE, body_group, macro_shape
from fit import fit, SOURCES
from features import facial_openings
from geom import mirror_map
from lashes import write_lashes
from repose import repose, transplant_hands
from retarget import deformation_gradients, gaussian_operator, Mapper, write_target

HEADER = ("# Cartoon hm08 offset target (tools/cartoon_hm08/build.py). CC0, derived from the\n"
          "# MakeHuman hm08 base and targets.  Vertex numbering is unchanged hm08.\n")


def sha(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def height(V, body):
    return float(np.ptp(V[body, 1]))


def blink_calibration(name, d_real, d_cart, VF, CF, rim_l, rim_r):
    """Scale a retargeted eyelid-closure unit so it closes the cartoon eye as far, in
    proportion to the opening, as the realistic unit closes the realistic eye."""
    def stats(V, d, rim):
        y = V[rim, 1]; up = rim[y > np.median(y)]; lo = rim[y <= np.median(y)]
        return np.abs(d[up, 1]).mean(), V[up, 1].mean() - V[lo, 1].mean()
    moved = [rim for rim in (rim_l, rim_r) if np.abs(d_real[rim]).max() > 1e-4]
    if "closure" not in name or not moved:
        return 1.0
    rim = moved[0]
    tr, hr = stats(VF, d_real, rim); tc, hc = stats(CF, d_cart, rim)
    return float((hc * tr / hr) / max(tc, 1e-6))


MUSCLE_SOURCES = {0: "Superhuman_F.glb", 1: "Superhuman_M.glb"}
MUSCLE_SMOOTH = 4  # Laplacian passes over the muscle correction


BUST_TARGET = "breast/female-young-maxmuscle-averageweight-maxcup-averagefirmness"
BUST_FALLOFF = 0.8  # dm: Gaussian falloff of the bust mask around the cup target's support
BUST_ARM_X = 2.0  # dm: the mask fades out before the arms


def bust_mask(o, V, adj):
    """Soft mask over everything the max-cup breast target moves, widened and feathered."""
    from hm08io import read_target, TARGETS
    i, d = read_target(TARGETS / (BUST_TARGET + ".target"))
    m = np.zeros(len(V)); m[i] = np.linalg.norm(d, axis=1)
    core = V[np.clip(m / (0.15 * m.max()), 0, 1) > 0.5]
    # Wide, smooth falloff from the bust (no crease where the body part takes over),
    # kept off the arms.
    from scipy.spatial import cKDTree
    d = cKDTree(core).query(V)[0]
    m = np.exp(-(d / BUST_FALLOFF) ** 2)
    m *= np.clip((BUST_ARM_X - np.abs(V[:, 0])) / 0.4, 0, 1)
    for _ in range(4):
        m = 0.5 * m + 0.5 * (adj @ m)
    return m[:, None]


def mesh_laplacian(o, n):
    """Row-normalised neighbour-average matrix over the hm08 face connectivity."""
    import scipy.sparse as sp
    rows, cols = [], []
    for f in o.faces:
        for i in range(len(f)):
            rows += [f[i], f[(i + 1) % len(f)]]; cols += [f[(i + 1) % len(f)], f[i]]
    A = sp.coo_matrix((np.ones(len(rows)), (rows, cols)), shape=(n, n)).tocsr()
    A.data[:] = 1
    deg = np.asarray(A.sum(1)).ravel(); deg[deg == 0] = 1
    return sp.diags(1 / deg) @ A


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=CE / "assets/cartoon_hm08")
    a = ap.parse_args(); t0 = time.time()
    fits = {sex: fit(sex) for sex in (0, 1)}
    o = fits[0][0]; VF, CF = fits[0][1], fits[0][3]; VM, CM = fits[1][1], fits[1][3]
    # Limb angles back to hm08's rest pose, which every clip is retargeted against.
    CF, CM = repose(VF, CF), repose(VM, CM)
    # hm08's own hands, scaled to the cartoon ones (fitting fuses the fingers).
    CF, CM = transplant_hands(VF, CF), transplant_hands(VM, CM)
    body = o.group_verts(body_group); n = len(VF)
    if a.out.exists():
        shutil.rmtree(a.out / "targets", ignore_errors=True)
        for stale in ("base.obj",):
            (a.out / stale).unlink(missing_ok=True)
    a.out.mkdir(parents=True, exist_ok=True)
    o.write(a.out / "base_female.obj", CF); o.write(a.out / "base_male.obj", CM)
    # The chest is split out so the editor can fade it in with age: a child should not
    # receive the adult Meshy bust or pectorals.
    def chest_mask(V):
        nip = V[[8451, 1779]]
        d = np.linalg.norm(V[:, None] - nip[None], axis=2).min(1)
        front = np.clip((V[:, 2] - (nip[:, 2].mean() - 0.9)) / 0.4, 0, 1)
        return (np.exp(-(d / 0.9) ** 2) * front)[:, None]
    def head_mask(V):
        neck = V[o.group_verts(lambda g: g == "joint-neck")].mean(0)[1]
        t = np.clip((V[:, 1] - (neck - 0.25)) / 0.5, 0, 1)
        return (t * t * (3 - 2 * t))[:, None]
    offsets = {}
    for label, V, C in (("female", VF, CF), ("male", VM, CM)):
        d = C - V; head = head_mask(V); chest = chest_mask(V) * (1 - head)
        offsets[f"cartoon/cartoon-{label}-head"] = d * head
        offsets[f"cartoon/cartoon-{label}"] = d * (1 - head - chest)
        offsets[f"cartoon/cartoon-{label}-chest"] = d * chest
    # Muscle (and female bust) corrections: at muscle=1 (breastSize=1 for the female) the
    # editor's realistic body plus the default cartoon offset should become the Superhuman
    # reference. The fit to it starts from MakeHuman's max-muscle shape; the remainder is
    # smoothed over the mesh (hm08 cannot follow the references' sharp muscle grooves
    # without folding) and split like the offsets. The head gets none.
    adj = mesh_laplacian(o, n)
    for label, sex, V, C in (("female", 0, VF, CF), ("male", 1, VM, CM)):
        V1 = macro_shape(o.v, sex, True)
        S = fit(sex, source=MUSCLE_SOURCES[sex], muscular=True)[3]
        S = transplant_hands(V1, repose(V1, S))
        k = S - (V1 + (C - V))
        for _ in range(MUSCLE_SMOOTH):
            k = 0.5 * k + 0.5 * (adj @ k)
        head = head_mask(V1); chest = chest_mask(V1)
        if sex == 0:
            # The female reference has both full muscle and full bust, and its fit starts
            # from MakeHuman's max cup. Everything that cup target moves (with a margin)
            # goes to the chest part, faded in by breast size, so at a smaller bust the
            # body part never reshapes the breasts.
            chest = np.maximum(chest, bust_mask(o, V1, adj))
        chest = chest * (1 - head)
        offsets[f"cartoon/cartoon-{label}-muscle"] = k * (1 - head - chest)
        offsets[f"cartoon/cartoon-{label}-muscle-chest"] = k * chest
        print(f"  {label} muscle correction: max {np.linalg.norm(k * (1 - head), axis=1).max():.3f} dm")
    for name, d in offsets.items():
        write_target(a.out / "targets" / (name + ".target"), np.arange(n), d, HEADER, False)
    print(f"shapes and offsets written ({time.time()-t0:.0f}s); expression corrections")

    # expression corrections, on the editor's own (quantised) units so names match
    G = gaussian_operator(o.v, 0.25)
    JN = Mapper(0.5 * (deformation_gradients(o, VF, CF) + deformation_gradients(o, VM, CM)), G)
    feat = facial_openings(o, VF); mir = mirror_map(o.v)
    rim_l = feat["eye_rim"]; rim_r = mir[rim_l]
    for label, C in (("female", CF), ("male", CM)):
        k, f = write_lashes(o, C, rim_l, mir, label, CE / f"assets/native/proxies/cartoon_eyelashes_{label}.pack")
        print(f"  {label} lash ribbon: {k} upper-lid vertices, {f} triangles")
    tmeta, tarr = mhp1.read(CE / "assets/native/targets.pack")
    corrections, scales = {}, {}
    for name, (idx, d0) in mhp1.decode_targets(tmeta, tarr).items():
        if not name.startswith("expression/"):
            continue
        r = JN(idx, d0)
        dense_r = np.zeros((n, 3)); dense_r[idx] = d0
        dense_c = np.zeros((n, 3)); dense_c[idx] = r
        k = blink_calibration(name, dense_r, dense_c, VF, CF, rim_l, rim_r)
        if k != 1.0:
            scales[name] = round(k, 3)
        corrections[name] = (idx, k * r - d0)
        write_target(a.out / "targets/cartoon" / (name + ".target"), idx, k * r - d0, HEADER, False)
    print(f"  {len(corrections)} expression corrections; eyelid closure scales {sorted(set(scales.values()))}")

    packed = dict(offsets.items()); packed = {k: (np.arange(n), v) for k, v in packed.items()}
    packed.update(corrections)
    meta, arrays = mhp1.encode_targets(packed)
    meta["cartoon"] = {"schema": "cartoon-hm08-offsets-v2", "female": "cartoon/cartoon-female",
                       "male": "cartoon/cartoon-male", "female_chest": "cartoon/cartoon-female-chest",
                       "male_chest": "cartoon/cartoon-male-chest", "female_head": "cartoon/cartoon-female-head",
                       "male_head": "cartoon/cartoon-male-head",
                       "female_muscle": "cartoon/cartoon-female-muscle",
                       "male_muscle": "cartoon/cartoon-male-muscle",
                       "female_muscle_chest": "cartoon/cartoon-female-muscle-chest",
                       "male_muscle_chest": "cartoon/cartoon-male-muscle-chest",
                       "reference_height_dm": {"female": height(VF, body), "male": height(VM, body)}}
    packs = CE / "assets/native/cartoon_hm08"
    for stale in ("base.pack", "targets.pack"):
        (packs / stale).unlink(missing_ok=True)
    mhp1.write(packs / "cartoon.pack", meta, arrays)

    report = {
        "schema": "cartoon-hm08-v2", "units": "makehuman-decimetres",
        "topology": "hm08 (19158 vertices, unchanged order/groups/UVs)",
        "sources": {s: {"file": SOURCES[i], "sha256": sha(ASSETS / SOURCES[i])} for i, s in ((0, "female"), (1, "male"))},
        "makehuman_base_sha256": sha(CE / "assets/makehuman/data/3dobjs/base.obj"),
        "fits": {LABEL: {k: v for k, v in fits[s][4].items() if k != "landmarks"} for s, LABEL in ((0, "female"), (1, "male"))},
        "reference_height_dm": meta["cartoon"]["reference_height_dm"],
        "eyelid_closure_scales": scales,
        "convention": __doc__.split("The cartoon look is")[1].split("Outputs")[0].strip(),
    }
    (a.out / "report.json").write_text(json.dumps(report, indent=1, default=float))
    print(f"done in {time.time()-t0:.0f}s")


if __name__ == "__main__":
    main()
