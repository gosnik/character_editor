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
from hm08io import Obj, ASSETS, CE, body_group
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
